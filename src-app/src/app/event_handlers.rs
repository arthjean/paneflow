//! Event-subscription callbacks and background workers for `PaneFlowApp`.
//!
//! Hosts the GPUI `subscribe` handlers (`handle_title_bar_event`,
//! `handle_pane_event`, `handle_terminal_event`) plus the port-scan /
//! loader-animation / stale-PID-sweep workers and the CWD change handler.
//!
//! Extracted from `main.rs` per US-026 of the src-app refactor PRD - pure
//! code-motion, behaviour unchanged.

use gpui::{App, AppContext, Context, Entity};
use notify::Watcher;

use crate::layout::{LayoutTree, MAX_PANES};
use crate::pane::{self, Pane};
use crate::pane_drag::DropEdge;
use crate::terminal::{self, TerminalView};
use crate::window_chrome::title_bar;
use crate::{PaneFlowApp, ai_types};

/// Cross-platform "is this PID still running?" probe used by the AI agent
/// stale-PID sweep. Unix path preserves the pre-US-034 `kill(pid, 0)` +
/// `ESRCH` semantics (EPERM ⇒ alive). Windows path mirrors the pattern in
/// `terminal::pty_session::Drop`: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`
/// returns NULL for a dead/inaccessible PID. US-034 - keeps `libc::` calls
/// off the Windows compile path.
fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid > i32::MAX as u32 {
            return false;
        }
        // SAFETY: `libc::kill` with sig=0 performs error-checking only and
        // does not deliver a signal. The call takes an i32 pid by value and
        // has no memory aliasing requirements.
        let ret = unsafe { libc::kill(pid as i32, 0) };
        if ret == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // ESRCH = no such process; EPERM/etc. ⇒ process exists but we
            // can't signal it - keep the entry.
            return errno != libc::ESRCH;
        }
        true
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::OpenProcess;
        // PROCESS_QUERY_LIMITED_INFORMATION (winnt.h: 0x1000) - minimum
        // access right that lets OpenProcess succeed for any visible PID.
        // Declared locally so we don't require an extra windows-sys feature.
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        if pid == 0 {
            return false;
        }
        // SAFETY: `OpenProcess` either returns a valid handle that we close,
        // or NULL. No memory aliasing. `CloseHandle` on a valid handle is
        // always sound.
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
        // Conservative fallback for exotic targets: never sweep. Better to
        // keep a stale entry than to drop a live one and confuse the AI
        // badge state.
        let _ = pid;
        true
    }
}

fn split_pane_at_edge(
    root: &mut LayoutTree,
    target: &Entity<Pane>,
    edge: DropEdge,
    new_pane: Entity<Pane>,
) {
    let (direction, swap) = edge.to_split();
    root.split_at_pane(target, direction, new_pane.clone());
    if swap {
        root.swap_panes(target, &new_pane);
    }
}

/// Parse the `starttime` field (22) from `/proc/{pid}/stat` content. The
/// comm field (2) is parenthesized and may contain spaces and parens
/// (`(tmux: server)`, `(next-server (v15))`) - split after the LAST `)`
/// (kernel-guaranteed unambiguous), then take the 20th whitespace field of
/// the remainder (state is field 3 → index 0, so starttime is index 19).
/// Platform-neutral pure parsing so the fixture test runs on every host.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse::<u64>().ok()
}

/// OS start time of a process, as an opaque value only ever compared for
/// equality. Pinned on `AgentSession` at creation and re-probed by the
/// sweep to detect PID reuse (a recycled PID passes `pid_is_alive` but
/// carries a different start time). `None` on probe failure (EPERM, dead
/// process, exotic target) - callers fall back to liveness-only.
#[cfg(target_os = "linux")]
pub(crate) fn pid_start_time(pid: u32) -> Option<u64> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat_starttime(&content)
}

#[cfg(target_os = "macos")]
pub(crate) fn pid_start_time(pid: u32) -> Option<u64> {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::proc_pid::pidinfo;
    // EPERM (SIP-protected targets) and dead-pid races degrade to None -
    // the caller keeps the conservative liveness-only check.
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
    // Same minimal access right as `pid_is_alive` above.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    if pid == 0 {
        return None;
    }
    // SAFETY: `OpenProcess` returns a valid handle (closed below) or NULL;
    // `GetProcessTimes` writes only into the four provided FILETIMEs, which
    // are plain-old-data and fully initialized by `zeroed`.
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

/// [`pid_is_alive`] hardened against PID reuse: when the session pinned a
/// start time at creation, a live PID with a DIFFERENT current start time
/// is a recycled PID - the original agent is gone. An unknown start time on
/// either side keeps the conservative "alive" answer.
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

impl PaneFlowApp {
    pub(crate) fn handle_title_bar_event(
        &mut self,
        _title_bar: Entity<title_bar::TitleBar>,
        event: &title_bar::TitleBarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            title_bar::TitleBarEvent::CloseRequested => {
                self.save_session(cx);
                // US-013 AC #2 - flush `app_exited` before the process is
                // torn down. Bounded to 2 s by the client; if PostHog is
                // unreachable the worker detaches and quit still proceeds.
                self.emit_app_exited_and_flush();
                cx.quit();
            }
            title_bar::TitleBarEvent::ToggleSidebar => {
                self.toggle_primary_sidebar(cx);
                self.title_bar_files_menu_open = None;
                self.title_bar_help_menu_open = None;
                if !self.primary_sidebar_visible {
                    self.workspace_menu_open = None;
                    self.profile_menu_open = None;
                    self.agents_view.agents_menu_open = None;
                    self.agents_view.sidebar_actions_menu_open = false;
                    self.agents_view.sidebar_mode_picker_open = false;
                }
            }
            title_bar::TitleBarEvent::ToggleRosettaSurface => {
                self.title_bar_files_menu_open = None;
                self.title_bar_help_menu_open = None;
                self.workspace_menu_open = None;
                self.tab_menu_open = None;
                self.profile_menu_open = None;
                self.files_menu_open = None;
                self.agents_view.agents_menu_open = None;
                self.agents_view.sidebar_actions_menu_open = false;
                self.agents_view.sidebar_mode_picker_open = false;

                if !self.rosetta_surface_allowed() {
                    self.reset_rosetta_surface_state();
                    cx.notify();
                    return;
                }

                if self.rosetta_surface_expanded {
                    self.rosetta_surface_expanded = false;
                    self.rosetta_surface_selected = 0;
                    self.rosetta_surface_selected_key = None;
                    self.rosetta_surface_pending_focus = false;
                } else {
                    let projection = self.rosetta_projection(std::time::Instant::now());
                    self.rosetta_surface_expanded = true;
                    self.rosetta_surface_selected = self
                        .rosetta_surface_selected
                        .min(projection.rows.len().saturating_sub(1));
                    self.rosetta_surface_selected_key = projection
                        .rows
                        .get(self.rosetta_surface_selected)
                        .map(crate::app::rosetta::RosettaRow::key);
                    self.rosetta_surface_pending_focus = true;
                }
                cx.notify();
            }
            title_bar::TitleBarEvent::ToggleFilesMenu(anchor) => {
                self.title_bar_files_menu_open =
                    self.title_bar_files_menu_open.is_none().then_some(*anchor);
                self.title_bar_help_menu_open = None;
                self.workspace_menu_open = None;
                self.tab_menu_open = None;
                self.profile_menu_open = None;
                self.agents_view.agents_menu_open = None;
                self.agents_view.sidebar_actions_menu_open = false;
                self.agents_view.sidebar_mode_picker_open = false;
                cx.notify();
            }
            title_bar::TitleBarEvent::ToggleHelpMenu(anchor) => {
                self.title_bar_help_menu_open =
                    self.title_bar_help_menu_open.is_none().then_some(*anchor);
                self.title_bar_files_menu_open = None;
                self.workspace_menu_open = None;
                self.tab_menu_open = None;
                self.profile_menu_open = None;
                self.agents_view.agents_menu_open = None;
                self.agents_view.sidebar_actions_menu_open = false;
                self.agents_view.sidebar_mode_picker_open = false;
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
                // Find the workspace that owns this pane (not necessarily the
                // active one - shells can exit in background workspaces).
                let ws_idx = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.root.as_ref().is_some_and(|r| r.contains_leaf(&pane)));
                let Some(ws_idx) = ws_idx else {
                    return;
                };

                // Remove this pane from the split tree
                if let Some(root) = self.workspaces[ws_idx].root.take() {
                    self.workspaces[ws_idx].root = root.remove_pane(&pane);
                }

                // Never leave a workspace without a pane - respawn at the
                // workspace's root cwd so the user returns to the right folder.
                if self.workspaces[ws_idx].root.is_none() {
                    let ws_id = self.workspaces[ws_idx].id;
                    let cwd = std::path::PathBuf::from(&self.workspaces[ws_idx].cwd);
                    let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, Some(cwd), None, cx));
                    cx.subscribe(&terminal, Self::handle_terminal_event)
                        .detach();
                    let new_pane = self.create_pane(terminal, ws_id, cx);
                    self.workspaces[ws_idx].root = Some(LayoutTree::Leaf(new_pane));
                    // The freshly-spawned replacement pane starts with an
                    // empty `custom_buttons` list - push the workspace's
                    // persisted set so the tab bar renders them again.
                    self.workspaces[ws_idx].propagate_custom_buttons(cx);
                }
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::ToggleAgentSessions => {
                // Toggle: clicking the icon again with the sidebar open closes it.
                if self.agent_sessions.sessions_sidebar_open {
                    self.close_sessions_sidebar(cx);
                    return;
                }
                // Open + bind + scan, extracted to `sessions_sidebar.rs` so a
                // workspace switch can re-target the open sidebar through the
                // exact same path.
                self.open_sessions_sidebar_for_pane(&pane, None, cx);
            }
            pane::PaneEvent::ToggleFilesSidebar => {
                // Open/close the docked Files tree for the active workspace's
                // folder. Mutual exclusion with the sessions sidebar is handled
                // inside `toggle_files_sidebar`.
                self.toggle_files_sidebar(cx);
            }
            pane::PaneEvent::CopySurfaceRef(surface_id) => {
                // US-010: resolve the globally-disambiguated name (matching the
                // MCP `list_panes` tool) and copy it to the clipboard. Fall back
                // to the raw id if the surface vanished between click and here.
                let sid = *surface_id;
                let reference = self
                    .collect_surface_meta(cx)
                    .into_iter()
                    .find(|m| m.surface_id == sid)
                    .map(|m| m.name)
                    .unwrap_or_else(|| sid.to_string());
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(reference.clone()));
                self.show_toast(format!("Copied surface ref: {reference}"), cx);
            }
            pane::PaneEvent::SurfaceRenamed => {
                // US-013: a tab's custom name changed - persist so it survives
                // restart (the name rides in the layout's SurfaceDefinition).
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::OpenTabMenu { tab_idx, position } => {
                // EP-002 US-006: open the "Move to pane…" menu for this tab.
                // Mutually exclusive with the other popovers, matching the
                // workspace/profile/sessions menu pattern.
                self.workspace_menu_open = None;
                self.profile_menu_open = None;
                self.tab_menu_open = Some(crate::TabContextMenu {
                    source_pane: pane.clone(),
                    tab_idx: *tab_idx,
                    position: *position,
                });
                cx.notify();
            }
            pane::PaneEvent::DropSplit {
                edge,
                source_pane,
                source_idx,
                duplicate,
            } => {
                let edge = *edge;
                let source_idx = *source_idx;
                let duplicate = *duplicate;
                let source_pane = source_pane.clone();
                let target = &pane; // the emitting pane is the split target

                // Resolve the workspace owning the target pane.
                let Some(ws_idx) = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.root.as_ref().is_some_and(|r| r.contains_leaf(target)))
                else {
                    return;
                };

                // A split adds one pane - refuse at the cap (edge case #5).
                if self.workspaces[ws_idx]
                    .root
                    .as_ref()
                    .map(|r| r.leaf_count())
                    .unwrap_or(0)
                    >= MAX_PANES
                {
                    return;
                }

                // Refuse a meaningless self-split: promoting a pane's *only*
                // tab onto its own edge just replaces it with an identical
                // single-tab pane (edge case #9). A pane with >1 tab can
                // legitimately split one tab out.
                if !duplicate && &source_pane == target && source_pane.read(cx).tabs.len() <= 1 {
                    return;
                }

                let ws_id = self.workspaces[ws_idx].id;

                // Build the new pane: a fresh terminal at the dragged tab's cwd
                // (duplicate, US-010) or the moved tab itself (US-009).
                let new_pane = if duplicate {
                    let cwd = source_pane
                        .read(cx)
                        .tabs
                        .get(source_idx)
                        .and_then(crate::pane::TabContent::as_terminal)
                        .and_then(|t| t.read(cx).terminal.cwd_now());
                    // Windows `cwd_now()` is always `None`; fall back to the
                    // workspace root instead of the process `current_dir()`.
                    let cwd = self.new_terminal_cwd(cwd);
                    let term = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
                    self.create_pane(term, ws_id, cx)
                } else {
                    let Some(tab) =
                        source_pane.update(cx, |src, _| src.take_tab_for_move(source_idx))
                    else {
                        return;
                    };
                    self.create_pane_with_existing_tab(tab, ws_id, cx)
                };

                if let Some(root) = &mut self.workspaces[ws_idx].root {
                    split_pane_at_edge(root, target, edge, new_pane.clone());
                }

                // Move-only: reflow away the source pane if it emptied.
                if !duplicate {
                    source_pane.update(cx, |src, src_cx| {
                        if src.tabs.is_empty() {
                            src_cx.emit(pane::PaneEvent::Remove);
                        } else {
                            src_cx.notify();
                        }
                    });
                }

                self.workspaces[ws_idx].propagate_custom_buttons(cx);
                // Focus the new pane on next render (no Window here).
                self.pending_pane_focus = Some(new_pane);
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::NewTerminalTab => {
                // The Pane can't spawn its own terminal at the right cwd (it
                // knows its `workspace_id` but not the directory) nor wire the
                // app-level CWD/port/service subscription, so it routes here.
                // Resolve the owning workspace by id and spawn at its cwd; on
                // Windows this is what keeps a new tab out of the process
                // `current_dir()` (`C:\Program Files\PaneFlow`).
                let ws_id = pane.read(cx).workspace_id;
                let cwd = self
                    .workspaces
                    .iter()
                    .find(|ws| ws.id == ws_id)
                    .map(|ws| ws.cwd.as_str())
                    .filter(|c| !c.is_empty())
                    .map(std::path::PathBuf::from);
                let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
                // App-level subscription so CWD/port/service events route
                // (mirrors `create_pane` / `DuplicateTabInto`); `add_tab` wires
                // the pane-level subscription.
                cx.subscribe(&terminal, Self::handle_terminal_event)
                    .detach();
                pane.update(cx, |p, cx| p.add_tab(terminal, cx));
                cx.notify();
            }
            pane::PaneEvent::DuplicateTabInto {
                source_pane,
                source_idx,
                dest_idx,
            } => {
                // EP-003 US-010: a strip/center drop with the duplicate modifier
                // held. Spawn a fresh terminal at the dragged tab's CWD and
                // insert it into the emitting (destination) pane; the original
                // stays put. Spawning here (not in the Pane) is required so the
                // app-level CWD/port subscription gets wired, exactly like the
                // `DropSplit` duplicate path and `create_pane`.
                let source_idx = *source_idx;
                let dest_idx = *dest_idx;
                let source_pane = source_pane.clone();
                let dest = pane.clone(); // the emitting pane is the destination

                // Resolve the workspace owning the destination pane (for ws_id).
                // A bail here also covers the race where `dest` was removed from
                // the tree between the drop emit and this handler.
                let Some(ws_idx) = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.root.as_ref().is_some_and(|r| r.contains_leaf(&dest)))
                else {
                    return;
                };
                let ws_id = self.workspaces[ws_idx].id;

                // CWD of the dragged terminal. `None` (non-terminal tab, or a
                // stale `source_idx`) → fresh terminal at the default cwd,
                // matching `DropSplit`'s duplicate path.
                let cwd = source_pane
                    .read(cx)
                    .tabs
                    .get(source_idx)
                    .and_then(crate::pane::TabContent::as_terminal)
                    .and_then(|t| t.read(cx).terminal.cwd_now());
                // Windows `cwd_now()` is always `None`; fall back to the workspace
                // root instead of the process `current_dir()`.
                let cwd = self.new_terminal_cwd(cwd);
                let term = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
                // App-level subscription so CWD/port/service events route
                // (mirrors `create_pane`); the pane-level subscription is wired
                // by `insert_duplicated_tab`.
                cx.subscribe(&term, Self::handle_terminal_event).detach();

                dest.update(cx, |p, cx| {
                    p.insert_duplicated_tab(crate::pane::TabContent::Terminal(term), dest_idx, cx);
                });
                self.workspaces[ws_idx].propagate_custom_buttons(cx);
                // Focus the destination pane (its newly-selected duplicate tab)
                // on next render (no Window here).
                self.pending_pane_focus = Some(dest);
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::DropSessionSplit {
                edge,
                agent,
                session_id,
                cwd,
            } => {
                // A session row was dropped out of the sidebar onto a pane.
                // Spawn a fresh terminal at the session's cwd running the
                // agent's resume command, then split the target pane toward the
                // previewed edge (or append it here as a tab for center).
                let edge = *edge;
                let agent = *agent;
                let session_id = session_id.clone();
                let cwd = cwd.clone();
                let target = pane.clone(); // the emitting pane is the target

                let Some(ws_idx) = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.root.as_ref().is_some_and(|r| r.contains_leaf(&target)))
                else {
                    return;
                };

                // A split adds one pane - refuse at the cap (edge case #5). A
                // center drop appends a tab to an existing pane, so it doesn't
                // grow the count and isn't capped.
                if edge.is_some()
                    && self.workspaces[ws_idx]
                        .root
                        .as_ref()
                        .map(|r| r.leaf_count())
                        .unwrap_or(0)
                        >= MAX_PANES
                {
                    return;
                }

                let ws_id = self.workspaces[ws_idx].id;
                let cwd_path = (!cwd.is_empty()).then(|| std::path::PathBuf::from(&cwd));
                let term = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd_path, None, cx));
                // Resume the picked session in the new terminal. Honors the
                // Claude bypass flag exactly like a tab-bar launch. Skips the
                // send if the id fails the allow-list (defence-in-depth).
                if let Some(resume) =
                    crate::app::sessions_sidebar::resume_command(agent, &session_id)
                {
                    term.read(cx).send_command(&resume);
                }

                match edge {
                    Some(edge) => {
                        // `create_pane` wires the app-level CWD/port subscription
                        // and the pane-event subscription (mirrors `DropSplit`).
                        let new_pane = self.create_pane(term, ws_id, cx);
                        if let Some(root) = &mut self.workspaces[ws_idx].root {
                            split_pane_at_edge(root, &target, edge, new_pane.clone());
                        }
                        self.workspaces[ws_idx].propagate_custom_buttons(cx);
                        self.pending_pane_focus = Some(new_pane);
                    }
                    None => {
                        // Center drop: append the resumed session as a new tab in
                        // the target pane (mirrors `DuplicateTabInto`).
                        cx.subscribe(&term, Self::handle_terminal_event).detach();
                        target.update(cx, |p, cx| {
                            let dest_idx = p.tabs.len();
                            p.insert_duplicated_tab(
                                crate::pane::TabContent::Terminal(term),
                                dest_idx,
                                cx,
                            );
                        });
                        self.workspaces[ws_idx].propagate_custom_buttons(cx);
                        self.pending_pane_focus = Some(target);
                    }
                }
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::DropMarkdownSplit { edge, path } => {
                // A markdown row was dropped out of the Files sidebar onto a
                // pane (EP-003 US-008). Open it via the existing `MarkdownView`
                // API, then split the target toward the previewed edge or append
                // it here as a tab (center). Mirrors `DropSessionSplit`, minus
                // the terminal spawn.
                let edge = *edge;
                let path = path.clone();
                let target = pane.clone(); // the emitting pane is the target

                let Some(ws_idx) = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.root.as_ref().is_some_and(|r| r.contains_leaf(&target)))
                else {
                    return;
                };

                // A split adds one pane - refuse at the cap (edge case #9). A
                // center drop appends a tab, so it isn't capped.
                if edge.is_some()
                    && self.workspaces[ws_idx]
                        .root
                        .as_ref()
                        .map(|r| r.leaf_count())
                        .unwrap_or(0)
                        >= MAX_PANES
                {
                    return;
                }

                let ws_id = self.workspaces[ws_idx].id;
                let markdown = cx.new(|cx| crate::markdown::MarkdownView::open(path, cx));

                match edge {
                    Some(edge) => {
                        let new_pane = self.create_pane_with_existing_tab(
                            crate::pane::TabContent::Markdown(markdown),
                            ws_id,
                            cx,
                        );
                        if let Some(root) = &mut self.workspaces[ws_idx].root {
                            split_pane_at_edge(root, &target, edge, new_pane.clone());
                        }
                        self.workspaces[ws_idx].propagate_custom_buttons(cx);
                        self.pending_pane_focus = Some(new_pane);
                    }
                    None => {
                        // Center drop: append the markdown as a new tab in the
                        // target pane (mirrors the click-to-open path).
                        target.update(cx, |p, cx| {
                            p.add_markdown_tab(markdown, cx);
                        });
                        self.pending_pane_focus = Some(target);
                    }
                }
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::Split(direction) => {
                let direction = *direction;
                if let Some(ws) = self.active_workspace()
                    && let Some(root) = &ws.root
                    && root.leaf_count() >= MAX_PANES
                {
                    self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
                    return;
                }
                // Inherit CWD and estimate initial grid size from the source terminal.
                // Grid is halved in the split direction; refined to exact size on first prepaint.
                // US-020: markdown panes have no terminal - fall back to the
                // workspace's root cwd and a default 80×24 grid so a split
                // request from a markdown pane still yields a usable terminal.
                let (source_cwd, initial_size) = match pane.read(cx).active_terminal_opt() {
                    Some(active) => {
                        let view = active.read(cx);
                        let cwd = view.terminal.cwd_now();
                        let term = view.terminal.term.lock();
                        let (cols, rows) = crate::terminal::types::grid_size(&term);
                        let size = match direction {
                            crate::layout::SplitDirection::Horizontal => (cols, (rows / 2).max(1)),
                            crate::layout::SplitDirection::Vertical => ((cols / 2).max(1), rows),
                        };
                        (cwd, size)
                    }
                    // Markdown pane (US-020): no terminal to read a cwd from, and
                    // a default grid. `new_terminal_cwd` supplies the workspace
                    // root below, exactly like a terminal whose `cwd_now()` is `None`.
                    None => (None, (80, 24)),
                };
                // `cwd_now()` is `None` for a markdown source and on platforms
                // without child-cwd introspection (always on Windows); fall back
                // to the workspace root so the split never lands in the process
                // `current_dir()` (`C:\Program Files\PaneFlow` when installed).
                let source_cwd = self.new_terminal_cwd(source_cwd);
                let ws_id = self.active_workspace().map(|ws| ws.id).unwrap_or(0);
                let new_terminal =
                    cx.new(|cx| TerminalView::with_cwd(ws_id, source_cwd, Some(initial_size), cx));
                let new_pane = self.create_pane(new_terminal, ws_id, cx);
                if let Some(ws) = self.active_workspace_mut()
                    && let Some(root) = &mut ws.root
                {
                    root.split_at_pane(&pane, direction, new_pane);
                }
                // The freshly-spawned pane starts with an empty
                // `custom_buttons` list - push the workspace's current set
                // so the new pane's tab bar matches its siblings.
                if let Some(ws) = self.workspaces.get(self.active_idx) {
                    ws.propagate_custom_buttons(cx);
                }
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Terminal event handling - push-based port detection and CWD tracking
    // -----------------------------------------------------------------------

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
                // EP-005 US-014: remember which terminal announced this port
                // so the next scan can cross-check the announcement against
                // the actual LISTEN owner (collision badge).
                terminal.update(cx, |view, _| view.terminal.note_announced_port(info.port));
                if let Some(ws_idx) = self.workspace_idx_for_terminal(&terminal, cx) {
                    let ws = &mut self.workspaces[ws_idx];
                    if merge_service_label(&mut ws.service_labels, info.clone())
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
                // EP-006 US-019: persist immediately so the zoom survives a
                // crash, not just a clean quit (SurfaceRenamed parity).
                self.save_session(cx);
            }
            terminal::TerminalEvent::FleetSearchRequested { query, regex } => {
                // EP-006 US-018: fan the query out to every pane.
                self.start_fleet_search(query.clone(), *regex, cx);
            }
            terminal::TerminalEvent::OpenCodePath { path, line, col } => {
                // Spawn the editor on the GPUI background executor so a
                // slow editor launch (cold VS Code, remote SSH editor)
                // never blocks the main thread. `open_at_location`
                // already log-swallows failures, so we don't need to
                // surface the result here.
                let path = path.clone();
                let line = *line;
                let col = *col;
                cx.background_executor()
                    .spawn(async move {
                        crate::editor::open_at_location(&path, line, col);
                    })
                    .detach();
            }
            terminal::TerminalEvent::ChildExited => {
                // The Pane's own subscription closes the tab; here we drop
                // the dying surface's agent sessions NOW instead of waiting
                // ≤30s for the sweep. Covers the paths where the shim's
                // `ai.exit`/`ai.session_end` never arrive (shim SIGKILLed,
                // agent launched without the shim).
                self.purge_sessions_for_surface(terminal.entity_id().as_u64(), cx);
            }
            // TitleChanged is handled by Pane's subscription
            _ => {}
        }
    }

    /// US-020 - append a markdown tab to the pane that owns `source_terminal`.
    ///
    /// The historical implementation split the layout vertically and created
    /// a dedicated markdown pane; the user feedback was that opening a doc
    /// shouldn't shrink the terminal real-estate. The current behaviour is to
    /// make markdown a peer tab inside the same pane - the user keeps the
    /// terminal+markdown pair via Ctrl+Tab / mouse-click, and the layout tree
    /// is untouched.
    fn open_markdown_in_pane(
        &mut self,
        source_terminal: &Entity<TerminalView>,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspace_idx_for_terminal(source_terminal, cx) else {
            return;
        };
        let source_pane = self.workspaces[ws_idx].root.as_ref().and_then(|root| {
            root.collect_leaves()
                .into_iter()
                .find(|pane| pane.read(cx).contains_terminal(source_terminal))
        });
        let Some(source_pane) = source_pane else {
            return;
        };

        let path_for_pane = path.clone();
        let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
            crate::markdown::MarkdownView::open(path_for_pane, cx)
        });

        source_pane.update(cx, |pane, cx| {
            pane.add_markdown_tab(markdown, cx);
            cx.notify();
        });
        self.save_session(cx);
        cx.notify();
    }

    /// Find which workspace contains the given terminal entity.
    fn workspace_idx_for_terminal(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &App,
    ) -> Option<usize> {
        self.workspaces.iter().position(|ws| {
            ws.root.as_ref().is_some_and(|root| {
                // U-013: zero-alloc - `any_leaf` short-circuits without the
                // `collect_leaves()` Vec<Entity<Pane>> clone the old form built.
                root.any_leaf(&mut |pane| pane.read(cx).contains_terminal(terminal))
            })
        })
    }

    /// Immediately drop agent sessions anchored to a dying surface (the
    /// shell behind it exited - its tab is closing), plus any real-PID
    /// session of the same pass whose process is already gone. Surgical
    /// complement to [`Self::sweep_stale_pids`]: same retention semantics,
    /// zero latency instead of ≤30s, no Stalled logic. An `Errored` session
    /// on the dying surface is dropped too - that matches the sweep's
    /// "sticky until its pane closes" contract, just without the wait.
    pub(crate) fn purge_sessions_for_surface(&mut self, surface_id: u64, cx: &mut Context<Self>) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                // Opportunistic: a session never resolved to a surface can
                // only be reaped through its PID - probe it now (the dying
                // shell may have taken the agent with it via SIGHUP).
                keep_session_after_surface_purge(surface_id, pid, session)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            // Same post-mutation trio as the sweep: drop orphan pane glows,
            // flush queued prompts stranded on the dead session, repaint.
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    /// Probe registered AI agent PIDs and clean up stale entries where the
    /// process no longer exists. See [`pid_is_alive`] for the per-platform
    /// probe (Unix: `kill(pid, 0)` / `ESRCH`; Windows: `OpenProcess` null
    /// handle; other: conservative keep).
    pub(crate) fn sweep_stale_pids(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        // EP-004 US-010: surfaces that still resolve to a live terminal tab.
        // An `Errored` session's PID is dead by definition (the binary
        // exited) - it is spared from the PID reap WHILE its pane lives so
        // the crash signal stays visible, and reaped here once the pane
        // closes. An Errored session that never resolved a surface has no
        // visible anchor beyond the sidebar; it follows the plain PID reap
        // (≤ 30 s) so unresolvable rows can never accumulate.
        let live_surfaces: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .filter_map(|ws| ws.root.as_ref())
            .flat_map(|root| root.collect_leaves())
            .flat_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .map(|t| t.entity_id().as_u64())
                    .collect::<Vec<_>>()
            })
            .collect();
        // EP-004 US-011 (cli-cockpit) + US-013 (agent-control-plane): Stalled
        // detection (default ON, threshold default 60 s, both hot-reload aware
        // via `cached_config`). The sweep runs every 30 s, so the effective
        // detection latency is threshold + up to 30 s granularity (documented
        // in the PRD AC and the JSON-schema description).
        let stall_enabled = self.cached_config.agent_stall_detection_enabled();
        let stall_threshold = std::time::Duration::from_secs(
            self.cached_config.resolved_agent_stall_threshold_secs(),
        );
        let active_workspace_id = if matches!(self.mode, paneflow_config::schema::AppMode::Cli) {
            self.workspaces.get(self.active_idx).map(|ws| ws.id)
        } else {
            None
        };
        let mut stalled_notifs: Vec<(crate::agent_launcher::TerminalAgent, String, u64, bool)> =
            Vec::new();
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            // Synthetic PIDs (from the upsert fallback for legacy shims
            // without `pid` on every frame) are stored in the high half
            // of u32 - outside the OS-assignable range on all supported
            // platforms - so probing them with `kill(pid, 0)` would
            // always say "dead" and immediately drop a live legacy
            // session. Keep them around: they'll be cleared by
            // `ai.session_end` or by the next state transition.
            ws.agent_sessions.retain(|&pid, session| {
                stale_sweep_keeps_without_pid_probe(pid, session, &live_surfaces)
                    || pid_matches(pid, session.proc_start)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
            // US-011: a `Thinking` session silent past the threshold flips
            // to `Stalled`. Only `Thinking` flips, so the once-per-episode
            // notification dedup is structural: the session stays Stalled
            // (this branch can't re-trigger) until a hook event revives it,
            // and a NEW episode requires a fresh Thinking phase first.
            if stall_enabled {
                for session in ws.agent_sessions.values_mut() {
                    if session
                        .state
                        .stalls_after(session.last_activity.elapsed(), stall_threshold)
                    {
                        session.state = ai_types::AgentState::Stalled;
                        // This write bypasses `upsert_session_state`, so hold
                        // its invariant by hand: only WaitingForInput carries
                        // a wait stamp. A Thinking row is already None, but
                        // clear defensively rather than rely on that.
                        session.waiting_since = None;
                        stalled_notifs.push((
                            session.tool,
                            ws.title.clone(),
                            session.last_activity.elapsed().as_secs(),
                            active_workspace_id == Some(ws.id),
                        ));
                        changed = true;
                    }
                }
            }
        }
        // Agents-view threads: a CLI killed mid-turn never sends `ai.stop`,
        // which would leave the row spinner running forever. Same
        // conservative policy as above - a thread whose hook frames carried
        // no PID is kept as-is (cleared by `ai.stop` / `ai.session_end`).
        for t in self
            .projects
            .iter_mut()
            .flat_map(|p| p.threads.iter_mut())
            .chain(self.chats.iter_mut())
        {
            if t.status != crate::project::ThreadStatus::Idle
                && let Some(pid) = t.agent_pid
                && !pid_matches(pid, t.agent_proc_start)
            {
                t.status = crate::project::ThreadStatus::Idle;
                t.agent_pid = None;
                t.agent_proc_start = None;
                changed = true;
            }
        }
        if changed {
            // US-018 (orchestration-v2): a swept session may have been
            // driving a pane glow - resync so no orphan attention survives.
            self.sync_attention(cx);
            // EP-001 US-003 (cli-cockpit): a swept `Thinking` session leaves
            // a bare shell - flush (or drop) its queued prompt now, else the
            // buffer and the "1 queued" chip strand forever (no further
            // `ai.*` frame will ever arrive for the dead session).
            self.agent_sessions_changed(cx);
            cx.notify();
        }
        // EP-004 US-011: fire AFTER the state writes so the notification and
        // the UI agree. One entry per Thinking→Stalled transition == one
        // notification per stall episode (PRD dedup AC).
        for (agent, title, silent_secs, source_visible) in stalled_notifs {
            super::ipc_handler::fire_stalled_notification(
                agent,
                &title,
                silent_secs,
                &self.cached_config,
                source_visible,
                cx.background_executor().clone(),
            );
        }
    }

    /// Schedule a debounced port-scan ladder for the given workspace.
    ///
    /// `port_scan_pending` absorbs bursts while a ladder is in flight: the
    /// old design bumped the generation on EVERY burst, so sustained output
    /// (an agent streaming for a minute) superseded the 500ms-debounced scan
    /// over and over and no scan ran until the terminal went quiet. The
    /// generation counter stays as the cancellation belt for workspace
    /// close/reuse.
    fn schedule_port_scan(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
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
                // Debounce: wait 500ms for activity to settle
                smol::Timer::after(std::time::Duration::from_millis(500)).await;

                // Burst scan at 0s, +2s, +6s after debounce
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

                // Re-arm regardless of how the ladder ended - the next
                // ActivityBurst starts a fresh one.
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, _cx| {
                        if let Some(ws) = app.workspaces.iter_mut().find(|ws| ws.id == ws_id) {
                            ws.port_scan_pending = false;
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

    /// Execute a single per-pane scan for a workspace (EP-005 US-012).
    /// Returns `false` if the scan should be aborted (generation superseded
    /// or workspace removed).
    fn run_port_scan(&mut self, ws_id: u64, generation: u64, cx: &mut Context<Self>) -> bool {
        let ws = match self.workspaces.iter().find(|ws| ws.id == ws_id) {
            Some(ws) if ws.port_scan_generation == generation => ws,
            _ => return false,
        };

        // (terminal entity id, PTY child pid) pairs - the scan partitions
        // the process walk per terminal subtree instead of flattening the
        // workspace into one pid pool.
        let roots: Vec<(u64, u32)> = ws
            .root
            .as_ref()
            .map(|root| {
                root.collect_leaves()
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
                    .collect()
            })
            .unwrap_or_default();

        if roots.is_empty() {
            return true;
        }

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // One unified subtree walk per tick feeds ports AND agent
                // identity (the pre-refactor code walked the descendants
                // once for each - this is the strictly-cheaper single pass,
                // US-012 cost contract).
                let scan = smol::unblock(move || {
                    let agent_binaries: Vec<&'static str> =
                        crate::agent_launcher::TerminalAgent::ALL
                            .iter()
                            .map(|a| a.binary())
                            .collect();
                    crate::workspace::scan_panes(&roots, &agent_binaries)
                })
                .await;
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

    /// Deposit a finished per-pane scan on the main thread (EP-005).
    ///
    /// Writes the per-terminal truth (identity-pill agent US-013, port
    /// badges + collision flags US-014) onto each LIVE terminal, then
    /// refreshes the workspace aggregates the sidebar reads - fed with the
    /// union of the per-pane results, identically to the pre-refactor flat
    /// scan (zero sidebar regression, US-012 AC). A pane closed between
    /// scan and deposit is naturally dropped: the deposit iterates the
    /// live tree, so its scan entry never matches.
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

        // Workspace aggregates (sidebar contract).
        let mut changed = merge_scan_workspace_state(
            &mut ws.active_ports,
            &mut ws.service_labels,
            &mut ws.detected_agents,
            &scan,
        );

        // Snapshot for the per-terminal announce-dedup purge below (ends the
        // mutable borrow region cleanly before the pane loop).
        let live_ports: Vec<u16> = ws.active_ports.clone();

        // Frontend URLs for live per-terminal service state (sidebar parity:
        // only frontend services get a link, backend ports stay textual).
        let frontend_urls: std::collections::HashMap<u16, String> = ws
            .service_labels
            .iter()
            .filter(|(_, info)| info.is_frontend)
            .filter_map(|(port, info)| info.url.clone().map(|u| (*port, u)))
            .collect();

        let leaves: Vec<gpui::Entity<crate::pane::Pane>> = ws
            .root
            .as_ref()
            .map(|root| root.collect_leaves())
            .unwrap_or_default();

        // US-014 collision pre-pass: port → owning terminal. A port
        // LISTENed by ≥ 2 subtrees is excluded - that is SO_REUSEPORT-style
        // sharding (nginx workers, `reusePort` servers), intentional load
        // balancing, not a collision. Other known false positives (proxies,
        // port-forwards, re-announcements after a restart) are tolerated in
        // v1 - the badge is an info-level heuristic, never blocking.
        let (owner, shared) = port_ownership(&scan);

        // Owner display names for the conflict tooltip (custom name, else
        // OSC title, else a stable surface reference). The OSC title is
        // UNTRUSTED terminal-controlled text and this tooltip is a new sink
        // for it: strip bidi/zero-width controls (an RLO could visually
        // reverse the surrounding `port N is owned by "…"` and spoof the
        // owner) and clamp the length (an unbounded title would otherwise
        // inflate the tooltip and this per-tick map). The custom name is
        // user-typed and already bounded, but it rides the same scrub -
        // one path, no exceptions.
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

        for pane in &leaves {
            let terminals: Vec<gpui::Entity<crate::terminal::TerminalView>> =
                pane.read(cx).terminals().cloned().collect();
            let mut pane_changed = false;
            for tv in terminals {
                let tid = tv.entity_id().as_u64();
                // A terminal spawned after the scan's root collection has no
                // entry - leave it untouched (the burst's next tick or the
                // next activity scan covers it).
                let Some(s) = scan.get(&tid) else {
                    continue;
                };
                let agent = s
                    .agents
                    .first()
                    .and_then(|b| crate::agent_launcher::TerminalAgent::from_binary(b));
                tv.update(cx, |view, _cx| {
                    let t = &mut view.terminal;
                    // A port that left LISTEN must become re-announceable -
                    // a dev server restarted inside a live shell (nodemon,
                    // plain re-run) re-prints its banner, and that line must
                    // re-fire ServiceDetected (the dedup was previously
                    // cleared only on ChildExit).
                    t.retain_reported_ports(&live_ports);
                    if t.detected_agent != agent || !t.agent_confirmed {
                        // The live scan owns the value from here on - this
                        // both confirms a restored "last known" pill and
                        // clears a stale one (US-013).
                        t.detected_agent = agent;
                        t.agent_confirmed = true;
                        pane_changed = true;
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
                // The tab strip renders from the terminals' state - nudge
                // the pane so the pill/badges repaint on this frame.
                pane.update(cx, |_, cx| cx.notify());
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }

    /// Handle a CWD change from a terminal. Only processes if the terminal is
    /// the active tab of a pane in its workspace (background terminals ignored).
    fn handle_cwd_change(
        &mut self,
        terminal: &Entity<TerminalView>,
        new_cwd: &str,
        cx: &mut Context<Self>,
    ) {
        // Find workspace where this terminal is the active tab in any pane.
        // US-020: skip markdown panes - they have no active terminal, so the
        // identity check via `active_terminal_opt` returns None for them.
        let ws_idx = self.workspaces.iter().position(|ws| {
            ws.root.as_ref().is_some_and(|root| {
                root.any_leaf(&mut |pane| {
                    pane.read(cx)
                        .active_terminal_opt()
                        .is_some_and(|t| *t == *terminal)
                })
            })
        });
        let Some(ws_idx) = ws_idx else { return };

        if self.workspaces[ws_idx].cwd == new_cwd {
            return;
        }

        // US-019: capture the stable workspace id, NOT the positional index.
        // The git probe below awaits (long on big repos / network FS); during
        // that await the main loop can run close/reorder/IPC-close and compact
        // the `Vec`, so a reused `ws_idx` would point at a *different*
        // workspace (silent git-state corruption + watch refcount desync).
        // Re-resolve the index by identity after the await - model:
        // `run_port_scan` / `spawn_initial_git_stats`.
        let ws_id = self.workspaces[ws_idx].id;

        let new_cwd_owned = new_cwd.to_string();

        // Run git probe off main thread
        cx.spawn({
            let new_cwd = new_cwd_owned.clone();
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (git_dir, branch, is_repo, stats) = smol::unblock({
                    let cwd = new_cwd.clone();
                    move || {
                        let git_dir = crate::workspace::find_git_dir(&cwd);
                        let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                        let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                        (git_dir, branch, is_repo, stats)
                    }
                })
                .await;

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        // Re-resolve by identity: the workspace may have been
                        // closed or reordered during the await.
                        let Some(ws_idx) = app.workspaces.iter().position(|ws| ws.id == ws_id)
                        else {
                            return;
                        };
                        // Unwatch old git dir
                        let old_git_dir = app.workspaces[ws_idx].git_dir.clone();
                        if let Some(ref dir) = old_git_dir {
                            app.unwatch_git_dir(dir);
                        }
                        // Update workspace git tracking (cwd stays fixed at creation -
                        // it represents the workspace's root folder and must not drift
                        // when the user `cd`s inside the shell).
                        let tracked_cwd = {
                            let ws = &mut app.workspaces[ws_idx];
                            ws.git_dir = git_dir.clone();
                            ws.cwd.clone()
                        };
                        // Watch new git dir
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
                            changed && app.refresh_agents_diff_if_open_for_cwd(&tracked_cwd, cx);
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

    /// US-013: populate a freshly-created workspace's `git diff --shortstat`
    /// stats off the GPUI main thread. The constructors build with
    /// `git_stats: default()` (0/0) so the blocking `git` subprocess never runs
    /// on the render thread; this spawns it via `smol::unblock` and re-injects
    /// the result, keyed by the stable `ws_id` (another workspace may be
    /// created/closed during the await - EP-003 identity model). Mirrors
    /// [`handle_cwd_change`].
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
                                && app.refresh_agents_diff_if_open_for_cwd(&cwd_for_apply, cx);
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
        announced_port_conflicts, keep_session_after_surface_purge, merge_scan_workspace_state,
        merge_service_label, parse_proc_stat_starttime, port_ownership,
        stale_sweep_keeps_without_pid_probe,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::terminal::ServiceInfo;
    use crate::workspace::{PaneScan, PortEntry};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn proc_stat_starttime_survives_hostile_comm_names() {
        // Plain comm: starttime is the 22nd field (9876543 here).
        let plain = "1234 (zsh) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 9876543 123 456 18446744073709551615";
        assert_eq!(parse_proc_stat_starttime(plain), Some(9876543));
        // Comm with spaces AND parens - split must anchor on the LAST ')'.
        let hostile = "1234 (next-server (v15)) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 424242 123 456";
        assert_eq!(parse_proc_stat_starttime(hostile), Some(424242));
        // Truncated content (fewer than 22 fields) yields None, not a panic.
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
            },
        )]);
        let (owner, shared) = port_ownership(&single_owner_scan);
        let display_names = HashMap::from([(1, "vite pane".to_string())]);

        assert_eq!(
            announced_port_conflicts(&[5173], 2, &owner, &shared, &display_names),
            vec![(5173, "vite pane".to_string())]
        );
    }
}
