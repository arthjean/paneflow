use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    Window, canvas, div, px,
};

use crate::PaneFlowApp;
use crate::app::diff_dock::code::controls::{EditorDisplay, editor_display, set_editor_display};
use crate::app::diff_dock::code::view::CodeView;
use crate::app::diff_dock::{ClosedFileTabs, DIFF_DOCK_PANEL_MIN_WIDTH, DiffDockData, DiffDockTab};
use paneflow_config::schema::{DockSession, DockTabSession};

const PANE_GRID_RESERVED_WIDTH: f32 =
    crate::layout::MIN_PANE_SIZE + 2. * crate::layout::PANE_GUTTER_PX;

const DIFF_DOCK_PANEL_FLOOR: f32 =
    DIFF_DOCK_PANEL_MIN_WIDTH + PANE_GRID_RESERVED_WIDTH + crate::app::constants::PANE_OUTER_GUTTER;

fn diff_dock_fit(preferred: f32, available: f32) -> (f32, f32) {
    let max =
        (available - PANE_GRID_RESERVED_WIDTH - crate::app::constants::PANE_OUTER_GUTTER).max(0.);
    (preferred.min(max), max)
}

fn main_panel_floor(dock_beside_grid: bool, panel_inset: f32) -> f32 {
    if dock_beside_grid {
        DIFF_DOCK_PANEL_FLOOR + panel_inset
    } else {
        0.
    }
}

fn diff_dock_maximized_width(available: f32, left_gutter: f32, right_gutter: f32) -> f32 {
    (available - left_gutter - right_gutter).max(DIFF_DOCK_PANEL_MIN_WIDTH)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PaneGridLayout {
    Flex,
    Clipped { visible: f32, full: f32 },
    Hidden,
}

impl PaneGridLayout {
    fn dock_left_gutter(self, gutter: f32) -> f32 {
        match self {
            Self::Flex => 0.,
            Self::Clipped { visible, full } => gutter * (1. - visible / full.max(1.)).clamp(0., 1.),
            Self::Hidden => gutter,
        }
    }
}

pub(crate) struct DiffDockSlot {
    open: bool,
    picker: bool,
    picked: bool,
    tabs: Vec<DiffDockTab>,
    active_tab: usize,
    data: Option<DiffDockData>,
    closed_files: ClosedFileTabs,
}

impl DiffDockSlot {
    fn is_idle(&self) -> bool {
        !self.open && !self.picked && self.tabs.is_empty() && self.closed_files.is_empty()
    }
}

fn stored_dock_path(root: &std::path::Path, path: &std::path::Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !root.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        _ => path.to_string_lossy().into_owned(),
    }
}

pub(crate) fn resolved_dock_path(root: &std::path::Path, stored: &str) -> std::path::PathBuf {
    let stored = std::path::Path::new(stored);
    if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        root.join(stored)
    }
}

fn dock_session_from(
    entries: Vec<Option<DockTabSession>>,
    active: usize,
    open: bool,
    tree: bool,
) -> Option<DockSession> {
    let active = kept_index(entries.iter().map(Option::is_some), active);
    let tabs: Vec<DockTabSession> = entries.into_iter().flatten().collect();
    let tree = tree
        && tabs
            .iter()
            .any(|tab| matches!(tab, DockTabSession::Files | DockTabSession::File { .. }));
    (open || !tabs.is_empty()).then_some(DockSession {
        open,
        tabs,
        active,
        tree,
    })
}

fn kept_index(kept: impl Iterator<Item = bool>, active: usize) -> usize {
    let kept: Vec<bool> = kept.collect();
    let before = |end: usize| kept[..end.min(kept.len())].iter().filter(|k| **k).count();
    if kept.get(active).copied().unwrap_or(false) {
        before(active)
    } else {
        before(active).saturating_sub(1)
    }
}

pub(crate) struct RestoredDockPlan {
    pub(crate) entries: Vec<RestoredDockEntry>,
    pub(crate) active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestoredDockEntry {
    Changes,
    Files,
    File(std::path::PathBuf),
}

pub(crate) fn restored_dock_plan(
    session: &DockSession,
    root: &std::path::Path,
    exists: impl Fn(&std::path::Path) -> bool,
) -> RestoredDockPlan {
    let candidates: Vec<Option<RestoredDockEntry>> = session
        .tabs
        .iter()
        .map(|tab| match tab {
            DockTabSession::Changes => Some(RestoredDockEntry::Changes),
            DockTabSession::Files => Some(RestoredDockEntry::Files),
            DockTabSession::File { path } => {
                let path = resolved_dock_path(root, path);
                exists(&path).then_some(RestoredDockEntry::File(path))
            }
            DockTabSession::Unknown => None,
        })
        .collect();
    let active = kept_index(candidates.iter().map(Option::is_some), session.active);
    let mut seen = std::collections::HashSet::new();
    let entries = candidates
        .into_iter()
        .flatten()
        .filter(|entry| match entry {
            RestoredDockEntry::File(path) => seen.insert(path.clone()),
            _ => true,
        })
        .collect::<Vec<_>>();
    let active = active.min(entries.len().saturating_sub(1));
    RestoredDockPlan { entries, active }
}

pub(crate) fn unsaved_file_views(
    tabs: &[DiffDockTab],
    cx: &gpui::App,
) -> Vec<gpui::Entity<CodeView>> {
    tabs.iter()
        .filter_map(|tab| match tab {
            DiffDockTab::File(view) if view.read(cx).is_dirty() => Some(view.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn take_unsaved_tabs(tabs: &mut Vec<DiffDockTab>, cx: &gpui::App) -> Vec<DiffDockTab> {
    let (unsaved, rest) = std::mem::take(tabs)
        .into_iter()
        .partition(|tab| matches!(tab, DiffDockTab::File(view) if view.read(cx).is_dirty()));
    *tabs = rest;
    unsaved
}

impl PaneFlowApp {
    fn dock_entries(
        tabs: &[DiffDockTab],
        root: &std::path::Path,
        cx: &gpui::App,
    ) -> Vec<Option<DockTabSession>> {
        tabs.iter()
            .map(|tab| match tab {
                DiffDockTab::Changes => Some(DockTabSession::Changes),
                DiffDockTab::PendingFile => Some(DockTabSession::Files),
                DiffDockTab::File(view) => Some(DockTabSession::File {
                    path: stored_dock_path(root, view.read(cx).path()),
                }),
                DiffDockTab::Terminal(_) => None,
            })
            .collect()
    }

    pub(crate) fn dock_session_for_tab(
        &self,
        tab_id: u64,
        ws_cwd: &str,
        tree: bool,
        cx: &gpui::App,
    ) -> Option<DockSession> {
        if let Some(pending) = self.diff_dock.restoring.get(&tab_id) {
            return Some(pending.clone());
        }
        let root = std::path::Path::new(ws_cwd);
        if self.diff_dock.owner == Some(tab_id) {
            return dock_session_from(
                Self::dock_entries(&self.diff_dock.diff_tabs, root, cx),
                self.diff_dock.diff_active_tab,
                self.diff_dock.open,
                tree,
            );
        }
        let slot = self.diff_dock.parked.get(&tab_id)?;
        dock_session_from(
            Self::dock_entries(&slot.tabs, root, cx),
            slot.active_tab,
            slot.open,
            tree,
        )
    }

    pub(crate) fn restore_dock_sessions(&mut self, cx: &mut Context<Self>) {
        let mut pending = Vec::new();
        for ws in &mut self.workspaces {
            let root = std::path::PathBuf::from(&ws.cwd);
            for tab in ws.tabs_mut() {
                if let Some(session) = tab.restored_dock.take() {
                    pending.push((tab.id, root.clone(), session));
                }
            }
        }
        if pending.is_empty() {
            return;
        }
        for (tab_id, _, session) in &pending {
            self.diff_dock.restoring.insert(*tab_id, session.clone());
        }
        let probes: Vec<std::path::PathBuf> = pending
            .iter()
            .flat_map(|(_, root, session)| {
                session.tabs.iter().filter_map(|tab| match tab {
                    DockTabSession::File { path } => Some(resolved_dock_path(root, path)),
                    _ => None,
                })
            })
            .collect();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let existing: std::collections::HashSet<std::path::PathBuf> =
                    smol::unblock(move || {
                        probes.into_iter().filter(|path| path.is_file()).collect()
                    })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    for (tab_id, root, session) in pending {
                        if app.diff_dock.restoring.remove(&tab_id).is_none() {
                            continue;
                        }
                        let plan =
                            restored_dock_plan(&session, &root, |path| existing.contains(path));
                        app.install_restored_dock(tab_id, session.open, plan, root, cx);
                    }
                    cx.notify();
                });
            },
        )
        .detach();
    }

    fn install_restored_dock(
        &mut self,
        tab_id: u64,
        open: bool,
        plan: RestoredDockPlan,
        root: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let tabs: Vec<DiffDockTab> = plan
            .entries
            .into_iter()
            .map(|entry| match entry {
                RestoredDockEntry::Changes => DiffDockTab::Changes,
                RestoredDockEntry::Files => DiffDockTab::PendingFile,
                RestoredDockEntry::File(path) => {
                    DiffDockTab::File(self.new_code_view(path, Some(root.clone()), true, cx))
                }
            })
            .collect();
        let live = self.diff_dock.owner == Some(tab_id)
            && self.diff_dock.diff_tabs.is_empty()
            && !self.diff_dock.open;
        if live {
            let has_tabs = !tabs.is_empty();
            self.diff_dock.diff_tabs = tabs;
            self.diff_dock.diff_active_tab = plan.active;
            self.diff_dock.picker = !has_tabs;
            self.diff_dock.picked = has_tabs;
            if open {
                let cwd = self.active_checkout().unwrap_or_default();
                self.open_diff_dock_panel(cwd, cx);
                self.diff_dock.reveal_animation = None;
            }
            return;
        }
        if self.diff_dock.owner == Some(tab_id) || self.diff_dock.parked.contains_key(&tab_id) {
            return;
        }
        let picked = !tabs.is_empty();
        self.diff_dock.parked.insert(
            tab_id,
            DiffDockSlot {
                open,
                picker: !picked,
                picked,
                tabs,
                active_tab: plan.active,
                data: None,
                closed_files: ClosedFileTabs::default(),
            },
        );
    }

    pub(crate) fn sync_diff_dock_session(&mut self, cx: &mut Context<Self>) {
        let active = self.active_session_id();
        if self.diff_dock.owner == active {
            return;
        }
        let previous = self.diff_dock.owner;
        self.diff_dock.owner = active;
        self.park_live_diff_dock(previous, cx);
        self.prune_parked_diff_docks(cx);
        self.restore_diff_dock(active, cx);
    }

    pub(crate) fn unsaved_views_of_tab(
        &self,
        tab_id: u64,
        cx: &gpui::App,
    ) -> Vec<gpui::Entity<CodeView>> {
        if self.diff_dock.owner == Some(tab_id) {
            return unsaved_file_views(&self.diff_dock.diff_tabs, cx);
        }
        self.diff_dock
            .parked
            .get(&tab_id)
            .map(|slot| unsaved_file_views(&slot.tabs, cx))
            .unwrap_or_default()
    }

    pub(crate) fn all_unsaved_views(&self, cx: &gpui::App) -> Vec<gpui::Entity<CodeView>> {
        let mut views = unsaved_file_views(&self.diff_dock.diff_tabs, cx);
        for slot in self.diff_dock.parked.values() {
            views.extend(unsaved_file_views(&slot.tabs, cx));
        }
        views
    }

    pub(crate) fn parked_diff_dock_data(&mut self, tab_id: u64) -> Option<&mut DiffDockData> {
        self.diff_dock.parked.get_mut(&tab_id)?.data.as_mut()
    }

    pub(crate) fn transfer_diff_dock(&mut self, from: u64, to: u64) {
        if self.diff_dock.owner == Some(from) {
            self.diff_dock.owner = Some(to);
        } else if let Some(slot) = self.diff_dock.parked.remove(&from) {
            self.diff_dock.parked.insert(to, slot);
        }
    }

    fn keep_rescued_diff_tabs(&mut self, rescued: Vec<DiffDockTab>, cx: &mut Context<Self>) {
        if rescued.is_empty() {
            return;
        }
        let count = rescued.len();
        self.diff_dock.diff_tabs.extend(rescued);
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.show_toast(
            format!(
                "Kept {} from a closed tab",
                super::plural(count, "unsaved file", "unsaved files")
            ),
            cx,
        );
    }

    pub(crate) fn active_session_id(&self) -> Option<u64> {
        self.active_workspace().map(|ws| ws.active_tab().id)
    }

    pub(crate) fn prune_parked_diff_docks(&mut self, cx: &mut Context<Self>) {
        let live: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs())
            .map(|tab| tab.id)
            .collect();
        let orphaned: Vec<u64> = self
            .diff_dock
            .parked
            .keys()
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        let mut rescued = Vec::new();
        for id in orphaned {
            if let Some(mut slot) = self.diff_dock.parked.remove(&id) {
                rescued.extend(take_unsaved_tabs(&mut slot.tabs, cx));
            }
        }
        self.keep_rescued_diff_tabs(rescued, cx);
    }

    fn park_live_diff_dock(&mut self, owner: Option<u64>, cx: &mut Context<Self>) {
        let slot = DiffDockSlot {
            open: self.diff_dock.open,
            picker: self.diff_dock.picker,
            picked: self.diff_dock.picked,
            tabs: std::mem::take(&mut self.diff_dock.diff_tabs),
            active_tab: std::mem::replace(&mut self.diff_dock.diff_active_tab, 0),
            data: self.diff_dock.data.take(),
            closed_files: std::mem::take(&mut self.diff_dock.closed_files),
        };
        self.close_diff_dock_panel(cx);
        self.diff_dock.picker = false;
        self.diff_dock.picked = false;
        self.diff_dock.diff_tab_close_armed = None;
        self.diff_dock.diff_options_menu_open = false;
        self.diff_dock.diff_new_tab_menu_open = false;
        self.diff_dock.diff_branch_menu = None;

        let owner = owner.filter(|id| {
            self.workspaces
                .iter()
                .flat_map(|ws| ws.tabs())
                .any(|tab| tab.id == *id)
        });
        match owner {
            None => {
                let mut slot = slot;
                let rescued = take_unsaved_tabs(&mut slot.tabs, cx);
                drop(slot);
                self.keep_rescued_diff_tabs(rescued, cx);
            }
            Some(id) if slot.is_idle() => {
                self.diff_dock.parked.remove(&id);
            }
            Some(id) => {
                self.diff_dock.parked.insert(id, slot);
            }
        }
    }

    fn restore_diff_dock(&mut self, session_id: Option<u64>, cx: &mut Context<Self>) {
        let Some(slot) = session_id.and_then(|id| self.diff_dock.parked.remove(&id)) else {
            return;
        };
        let rescued = std::mem::take(&mut self.diff_dock.diff_tabs);
        self.diff_dock.picker = slot.picker && rescued.is_empty();
        self.diff_dock.picked = slot.picked || !rescued.is_empty();
        self.diff_dock.diff_tabs = slot.tabs;
        self.diff_dock.diff_tabs.extend(rescued);
        self.diff_dock.diff_active_tab = slot.active_tab;
        self.diff_dock.closed_files = slot.closed_files;
        let cwd = slot
            .data
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.is_empty());
        self.diff_dock.data = slot.data;
        if slot.open {
            let cwd = cwd.or_else(|| self.active_checkout()).unwrap_or_default();
            self.open_diff_dock_panel(cwd, cx);
            self.diff_dock.reveal_animation = None;
        }
    }

    pub(crate) fn flush_shell(&self) -> bool {
        matches!(
            self.cached_config.resolved_interface_style(),
            paneflow_config::schema::InterfaceStyle::Classic
        )
    }

    pub(crate) fn diff_dock_toggle_available(&self) -> bool {
        self.settings_section.is_none() && self.active_workspace().is_some()
    }

    pub(crate) fn handle_toggle_diff_dock(
        &mut self,
        _: &crate::ToggleDiffDock,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_diff_dock(cx);
    }

    pub(crate) fn toggle_diff_dock(&mut self, cx: &mut Context<Self>) {
        if !self.diff_dock_toggle_available() {
            return;
        }
        if self.diff_dock.open {
            self.dismiss_diff_dock_panel(cx);
            return;
        }
        let Some(cwd) = self
            .active_checkout()
            .or_else(|| self.active_workspace().map(|ws| ws.cwd.clone()))
        else {
            return;
        };
        self.diff_dock.picker = !self.diff_dock.picked;
        self.diff_dock.picker_focus_pending = self.diff_dock.picker;
        self.open_diff_dock_panel(cwd, cx);
    }

    pub(crate) fn handle_toggle_diff_dock_maximize(
        &mut self,
        _: &crate::ToggleDiffDockMaximize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.diff_dock_visible() {
            return;
        }
        self.toggle_diff_dock_maximize(window, cx);
    }

    pub(crate) fn toggle_diff_dock_maximize(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.resize = None;
        let now = std::time::Instant::now();
        let full = self.diff_dock.pane_grid_width.get();
        let from = match self.diff_dock.maximize_animation {
            Some(animation) => animation.width_at(now),
            None if self.diff_dock.maximized.is_some() => 0.,
            None => full,
        };
        let to = match self.diff_dock.maximized.take() {
            Some(previous_focus) => {
                self.diff_dock.focus_after_restore = Some(previous_focus);
                full
            }
            None => {
                self.diff_dock.focus_after_restore = None;
                self.diff_dock.maximized = Some(crate::FocusReturn::capture(window, cx));
                self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
                0.
            }
        };
        self.diff_dock.maximize_animation = if !crate::ui_primitives::reduce_motion()
            && (from - to).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA
        {
            Some(crate::SidebarWidthAnimation {
                from_width: from,
                to_width: to,
                started_at: now,
            })
        } else {
            None
        };
        cx.notify();
    }

    pub(crate) fn apply_editor_display(&mut self, cx: &mut Context<Self>) {
        let display = EditorDisplay::from_config(&self.cached_config.editor);
        if display == editor_display(cx) {
            return;
        }
        let servers_changed = display.semantic_tokens != editor_display(cx).semantic_tokens;
        set_editor_display(display, cx);
        let live = self.diff_dock.diff_tabs.iter();
        let parked = self
            .diff_dock
            .parked
            .values()
            .flat_map(|slot| slot.tabs.iter());
        for tab in live.chain(parked) {
            if let DiffDockTab::File(view) = tab {
                view.update(cx, |view, cx| {
                    if servers_changed {
                        view.refresh_language_server(cx);
                    }
                    cx.notify();
                });
            }
        }
    }

    pub(crate) fn settle_dock_restore_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.diff_dock.maximize_animation.is_some() {
            return;
        }
        if let Some(origin) = self.diff_dock.focus_after_restore.take() {
            self.return_focus(&origin, window, cx);
        }
    }

    pub(crate) fn diff_dock_fills_panel(&self) -> bool {
        self.diff_dock.maximized.is_some() || self.diff_dock.maximize_animation.is_some()
    }

    fn rendered_dock_reveal(&mut self, window: &mut Window) -> Option<f32> {
        let animation = self.diff_dock.reveal_animation?;
        let now = std::time::Instant::now();
        if animation.is_finished(now) {
            if animation.to_width == 0. {
                return Some(0.);
            }
            self.diff_dock.reveal_animation = None;
            return None;
        }
        window.request_animation_frame();
        Some(animation.width_at(now))
    }

    fn rendered_pane_grid_layout(&mut self, window: &mut Window) -> PaneGridLayout {
        let now = std::time::Instant::now();
        if let Some(animation) = self.diff_dock.maximize_animation {
            if animation.is_finished(now) {
                self.diff_dock.maximize_animation = None;
                if self.diff_dock.focus_after_restore.is_some() {
                    window.request_animation_frame();
                }
            } else {
                window.request_animation_frame();
                return PaneGridLayout::Clipped {
                    visible: animation.width_at(now),
                    full: animation.from_width.max(animation.to_width),
                };
            }
        }
        if self.diff_dock.maximized.is_some() {
            PaneGridLayout::Hidden
        } else {
            PaneGridLayout::Flex
        }
    }

    pub(crate) fn main_panel_floor(&self) -> f32 {
        let panel_inset = if self.flush_shell() {
            0.
        } else {
            crate::app::constants::PANEL_INSET
        };
        main_panel_floor(self.diff_dock_beside_grid(), panel_inset)
    }

    pub(crate) fn diff_dock_visible(&self) -> bool {
        self.diff_dock.open && self.settings_section.is_none()
    }

    pub(crate) fn diff_dock_beside_grid(&self) -> bool {
        self.diff_dock_visible() || self.diff_dock_closing()
    }

    fn diff_dock_closing(&self) -> bool {
        !self.diff_dock.open
            && self.settings_section.is_none()
            && self
                .diff_dock
                .reveal_animation
                .is_some_and(|animation| animation.to_width == 0.)
    }

    pub(crate) fn wrap_cli_diff_dock(
        &mut self,
        body: AnyElement,
        slot: DockSlot,
        density: crate::layout::PaneDensity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let DockSlot {
            available_width,
            left_gutter,
            top: dock_top,
        } = slot;
        self.sync_files_sidebar_session(cx);
        let files_progress = self.rendered_files_sidebar_progress(window, cx);
        self.diff_dock.title_bar_zone_left = None;
        if !self.diff_dock_visible() && !self.diff_dock_closing() {
            return body;
        }
        let flush_shell = self.flush_shell();
        let ui = crate::theme::ui_colors();
        let grid = self.rendered_pane_grid_layout(window);
        let fills_panel = grid != PaneGridLayout::Flex;
        let outer_gutter = density.outer_gutter_px();
        let (width, max_width) = if fills_panel {
            let width = diff_dock_maximized_width(available_width, left_gutter, outer_gutter);
            (width, width)
        } else {
            diff_dock_fit(self.diff_dock.width, available_width)
        };
        let measured_grid_width = self.diff_dock.pane_grid_width.clone();
        let reveal = if fills_panel {
            None
        } else {
            self.rendered_dock_reveal(window)
        };
        let dock_column_width = width + outer_gutter;
        if flush_shell {
            self.diff_dock.title_bar_zone_left = Some(title_bar_zone_left(
                grid,
                available_width,
                dock_column_width,
                reveal,
            ));
        }
        let dock_frame = density.frame(gpui::Edges {
            top: true,
            right: true,
            bottom: true,
            left: grid == PaneGridLayout::Hidden,
        });
        div()
            .size_full()
            .flex()
            .flex_row()
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _w, cx| {
                if this.diff_dock.h_scroll_drag.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_diff_dock_h_scrollbar(event.position.x, cx);
                    } else {
                        this.end_diff_dock_h_scrollbar_drag(cx);
                    }
                } else if this.diff_dock.resize.is_some() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_diff_dock_resize(f32::from(event.position.x), cx);
                    } else {
                        this.end_diff_dock_resize(cx);
                    }
                } else {
                    this.update_diff_dock_hover(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, cx| {
                    this.end_diff_dock_h_scrollbar_drag(cx);
                    this.end_diff_dock_resize(cx);
                }),
            )
            .map(|row| match grid {
                PaneGridLayout::Flex => row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    measured_grid_width.set(f32::from(bounds.size.width))
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .child(body),
                ),
                PaneGridLayout::Clipped { visible, full } => row.child(
                    div()
                        .flex_none()
                        .w(px(visible))
                        .h_full()
                        .overflow_hidden()
                        .child(div().w(px(full)).h_full().child(body)),
                ),
                PaneGridLayout::Hidden => row,
            })
            .child({
                let dock = div()
                    .map(|dock| {
                        if fills_panel {
                            dock.flex_1().min_w_0()
                        } else {
                            dock.flex_none()
                        }
                    })
                    .pl(px(grid.dock_left_gutter(left_gutter)))
                    .h_full()
                    .relative()
                    .flex()
                    .flex_col()
                    .pt(dock_top)
                    .pb(px(outer_gutter))
                    .pr(px(outer_gutter))
                    .child(self.render_diff_dock_panel(
                        width,
                        max_width,
                        files_progress,
                        dock_frame,
                        ui,
                        window,
                        cx,
                    ))
                    .when(flush_shell && grid != PaneGridLayout::Hidden, |dock| {
                        dock.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(1.))
                                .bg(ui.border),
                        )
                    });
                match reveal {
                    Some(progress) => div()
                        .relative()
                        .flex_none()
                        .h_full()
                        .w(px(dock_column_width * progress))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(dock_column_width))
                                .child(dock),
                        )
                        .into_any_element(),
                    None => dock.into_any_element(),
                }
            })
            .into_any_element()
    }
}

pub(crate) struct DockSlot {
    pub(crate) available_width: f32,
    pub(crate) left_gutter: f32,
    pub(crate) top: gpui::Pixels,
}

fn title_bar_zone_left(
    grid: PaneGridLayout,
    available_width: f32,
    dock_column_width: f32,
    reveal: Option<f32>,
) -> f32 {
    match grid {
        PaneGridLayout::Flex => {
            (available_width - dock_column_width * reveal.unwrap_or(1.)).max(0.)
        }
        PaneGridLayout::Clipped { visible, .. } => visible.max(0.),
        PaneGridLayout::Hidden => 0.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_bar_zone_starts_where_the_dock_column_does() {
        assert_eq!(
            title_bar_zone_left(PaneGridLayout::Flex, 1200., 540., None),
            660.
        );
        assert_eq!(
            title_bar_zone_left(PaneGridLayout::Flex, 1200., 540., Some(0.5)),
            930.
        );
        assert_eq!(
            title_bar_zone_left(PaneGridLayout::Hidden, 1200., 1200., None),
            0.
        );
        assert_eq!(
            title_bar_zone_left(
                PaneGridLayout::Clipped {
                    visible: 300.,
                    full: 660.
                },
                1200.,
                1200.,
                None
            ),
            300.
        );
    }

    #[gpui::test]
    fn a_dock_without_an_owner_hands_back_every_unsaved_file(cx: &mut gpui::TestAppContext) {
        use gpui::{AppContext as _, EntityInputHandler as _};

        let (dirty, cx) = cx.add_window_view(|_window, cx| {
            CodeView::ready_for_test("/repo/dirty.rs".into(), "fn main() {}\n", cx)
        });
        dirty.update_in(cx, |view, window, cx| {
            view.replace_text_in_range(None, "edited ", window, cx);
        });
        let clean = cx.new(|cx| CodeView::ready_for_test("/repo/clean.rs".into(), "ok\n", cx));
        let mut tabs = vec![
            DiffDockTab::Changes,
            DiffDockTab::File(clean.clone()),
            DiffDockTab::File(dirty.clone()),
        ];

        let (unsaved, rescued) = cx.update(|_window, cx| {
            (
                unsaved_file_views(&tabs, cx),
                take_unsaved_tabs(&mut tabs, cx),
            )
        });

        assert_eq!(unsaved, vec![dirty.clone()]);
        assert_eq!(rescued.len(), 1, "only the modified file is rescued");
        assert!(matches!(&rescued[0], DiffDockTab::File(view) if *view == dirty));
        assert_eq!(tabs.len(), 2, "the rest of the slot can be dropped");
        assert!(
            cx.update(|_window, cx| dirty.read(cx).is_dirty()),
            "the rescued view keeps its edits"
        );
    }

    fn slot(open: bool, picked: bool, tabs: usize) -> DiffDockSlot {
        DiffDockSlot {
            open,
            picker: false,
            picked,
            tabs: (0..tabs).map(|_| DiffDockTab::Changes).collect(),
            active_tab: 0,
            data: None,
            closed_files: ClosedFileTabs::default(),
        }
    }

    #[test]
    fn a_dock_session_keeps_its_order_and_active_tab_without_terminals() {
        let file = |path: &str| {
            Some(DockTabSession::File {
                path: path.to_string(),
            })
        };
        let session = dock_session_from(
            vec![
                Some(DockTabSession::Changes),
                None,
                file("src/a.rs"),
                file("/etc/hosts"),
            ],
            3,
            true,
            true,
        )
        .expect("a session");
        assert_eq!(session.active, 2, "the terminal before it is dropped");
        assert_eq!(session.tabs.len(), 3);
        assert!(session.tree, "the tree was open beside the file tabs");
        let on_terminal =
            dock_session_from(vec![Some(DockTabSession::Changes), None], 1, false, true)
                .expect("kept");
        assert_eq!(on_terminal.active, 0);
        assert!(!on_terminal.tree, "no file tab, so no tree to bring back");
        let hidden = dock_session_from(vec![file("src/a.rs")], 0, true, false).expect("kept");
        assert!(!hidden.tree, "a tree the user hid stays hidden");
        assert_eq!(dock_session_from(vec![None], 0, false, false), None);
        assert_eq!(
            stored_dock_path(
                std::path::Path::new("/repo"),
                &std::path::Path::new("/repo").join("src").join("a.rs")
            ),
            std::path::Path::new("src").join("a.rs").to_string_lossy()
        );
        assert_eq!(
            stored_dock_path(
                std::path::Path::new("/repo"),
                std::path::Path::new("/etc/hosts")
            ),
            "/etc/hosts"
        );
    }

    #[test]
    fn a_restored_dock_omits_missing_files_and_keeps_fifty_in_order() {
        let root = std::path::Path::new("/repo");
        let mut tabs: Vec<DockTabSession> = (0..50)
            .map(|index| DockTabSession::File {
                path: format!("src/{index}.rs"),
            })
            .collect();
        tabs.insert(0, DockTabSession::Changes);
        let session = DockSession {
            open: true,
            tabs,
            active: 50,
            tree: true,
        };
        let plan = restored_dock_plan(&session, root, |_| true);
        assert_eq!(plan.entries.len(), 51);
        assert_eq!(plan.active, 50);
        assert_eq!(
            plan.entries[50],
            RestoredDockEntry::File(root.join("src/49.rs"))
        );

        let gone = root.join("src/49.rs");
        let plan = restored_dock_plan(&session, root, |path| path != gone);
        assert_eq!(plan.entries.len(), 50);
        assert_eq!(plan.active, 49, "the neighbour takes the active slot");
        assert!(!plan.entries.contains(&RestoredDockEntry::File(gone)));

        let unknown = DockSession {
            open: false,
            tabs: vec![DockTabSession::Unknown, DockTabSession::Files],
            active: 0,
            tree: true,
        };
        let plan = restored_dock_plan(&unknown, root, |_| true);
        assert_eq!(plan.entries, vec![RestoredDockEntry::Files]);
        assert_eq!(plan.active, 0);
    }

    #[test]
    fn a_wide_panel_leaves_the_preferred_dock_width_alone() {
        assert_eq!(diff_dock_fit(880., 1920.).0, 880.);
    }

    #[test]
    fn opening_a_right_rail_shrinks_the_dock_instead_of_clipping_it() {
        let available = 970.;
        let (width, max) = diff_dock_fit(880., available);
        assert!(width < 880., "the dock must give ground: {width}");
        assert_eq!(width, max, "a clamped dock renders at its ceiling");
        assert!(
            width + PANE_GRID_RESERVED_WIDTH + crate::app::constants::PANE_OUTER_GUTTER
                <= available,
            "the dock still overflows the panel: {width}"
        );
    }

    #[test]
    fn a_panel_too_narrow_for_the_dock_floor_keeps_the_pane_grid_reserve() {
        let available = 400.;
        let (width, max) = diff_dock_fit(540., available);
        assert_eq!(width, max);
        assert!(width < DIFF_DOCK_PANEL_MIN_WIDTH);
        assert_eq!(
            available - width - crate::app::constants::PANE_OUTER_GUTTER,
            PANE_GRID_RESERVED_WIDTH,
            "the pane grid keeps its reserve: {width}"
        );
        assert_eq!(diff_dock_fit(540., 0.).0, 0.);
    }

    #[test]
    fn a_main_panel_at_the_floor_holds_the_dock_floor_and_the_pane_grid_reserve() {
        let panel_inset = crate::app::constants::PANEL_INSET;
        let floor = main_panel_floor(true, panel_inset);
        let (width, _) = diff_dock_fit(540., floor - panel_inset);
        assert_eq!(width, DIFF_DOCK_PANEL_MIN_WIDTH);
        assert_eq!(main_panel_floor(false, panel_inset), 0.);
    }

    #[test]
    fn a_maximized_dock_spans_the_panel_minus_the_gutters() {
        let available = 1920.;
        let left_gutter = crate::app::constants::PANE_OUTER_GUTTER;
        let right_gutter = crate::app::constants::PANE_OUTER_GUTTER;
        assert_eq!(
            diff_dock_maximized_width(available, left_gutter, right_gutter),
            available - left_gutter - right_gutter
        );
        assert!(
            diff_dock_maximized_width(available, left_gutter, right_gutter)
                > diff_dock_fit(880., available).1
        );
        assert_eq!(
            diff_dock_maximized_width(available, 0., 0.),
            available,
            "a compact dock spans the whole panel"
        );
    }

    #[test]
    fn the_dock_gutter_grows_as_the_pane_grid_is_clipped_away() {
        let gutter = crate::layout::PANE_GUTTER_PX;
        assert_eq!(PaneGridLayout::Flex.dock_left_gutter(gutter), 0.);
        assert_eq!(PaneGridLayout::Hidden.dock_left_gutter(gutter), gutter);
        let half = PaneGridLayout::Clipped {
            visible: 400.,
            full: 800.,
        };
        assert!((half.dock_left_gutter(gutter) - gutter / 2.).abs() < 1e-3);
        let gone = PaneGridLayout::Clipped {
            visible: 0.,
            full: 800.,
        };
        assert_eq!(gone.dock_left_gutter(gutter), gutter);
    }

    #[test]
    fn a_maximized_dock_never_drops_below_the_floor() {
        assert_eq!(
            diff_dock_maximized_width(200., 0., 0.),
            DIFF_DOCK_PANEL_MIN_WIDTH
        );
    }

    #[test]
    fn a_session_that_never_opened_the_dock_parks_nothing() {
        assert!(slot(false, false, 0).is_idle());
    }

    #[test]
    fn a_dock_worth_restoring_is_parked() {
        assert!(!slot(true, false, 1).is_idle(), "an open dock must survive");
        assert!(
            !slot(false, true, 1).is_idle(),
            "an answered picker must not ask again"
        );
        assert!(
            !slot(false, false, 1).is_idle(),
            "a terminal / file tab must not be dropped"
        );
    }
}
