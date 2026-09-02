mod context_menu;
mod filter;
mod keyboard;
mod row;
mod view;
mod watch;

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, Window,
    div, px,
};

use crate::app::files_tree::{self, FilesTreeState};
use crate::{PaneFlowApp, ToggleFilesSidebar};

pub(crate) const FILES_SIDEBAR_WIDTH: f32 = 300.;
pub(super) const SIDEBAR_WIDTH: Pixels = px(FILES_SIDEBAR_WIDTH);
pub(super) const ROW_HEIGHT: Pixels = px(28.);
pub(super) const INDENT_STEP: f32 = 18.;
pub(super) const ROW_SLOT: f32 = 14.;
pub(super) const ROW_GAP: f32 = 12.;
pub(super) const DIMMED_OPACITY: f32 = 0.55;

impl PaneFlowApp {
    pub(crate) fn files_sidebar_host_visible(&self) -> bool {
        self.settings_section.is_none()
            && matches!(self.mode, paneflow_config::schema::AppMode::Cli)
    }

    pub(crate) fn handle_toggle_files_sidebar(
        &mut self,
        _: &ToggleFilesSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.files_sidebar_host_visible() {
            return;
        }
        if !self.files_sidebar_open {
            self.files_surface_id = self
                .workspaces
                .get(self.active_idx)
                .and_then(|ws| ws.active_tab().root.as_ref())
                .and_then(|root| root.focused_pane(window, cx))
                .and_then(|pane| pane.read(cx).active_terminal_opt())
                .map(|terminal| terminal.entity_id().as_u64());
        }
        self.toggle_files_sidebar(cx);
        if self.files_sidebar_open {
            self.files_focus.focus(window, cx);
        }
    }

    pub(crate) fn toggle_files_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.files_sidebar_open {
            self.close_files_sidebar(cx);
            return;
        }
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        let persisted = ws.files_expanded.clone();

        if self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some()
        {
            self.close_sessions_sidebar_immediate(cx);
        }
        self.dismiss_transient_surfaces();

        self.set_files_sidebar_open(true, cx);
        self.files_tree_scroll = gpui::ScrollHandle::new();
        self.files_selected = 0;
        self.files_filter_input
            .update(cx, |input, cx| input.clear(cx));
        self.spawn_files_hydration(root, persisted, cx);
    }

    pub(crate) fn close_files_sidebar(&mut self, cx: &mut Context<Self>) {
        self.files_watcher = None;
        self.files_event_rx = None;
        self.files_menu_open = None;
        self.set_files_sidebar_open(false, cx);
    }

    fn files_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if let Some(animation) = self.files_sidebar_animation {
            animation.width_at(now)
        } else if self.files_sidebar_open {
            FILES_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(crate) fn rendered_files_sidebar_width(&mut self, window: &mut Window) -> f32 {
        let now = std::time::Instant::now();
        if let Some(animation) = self.files_sidebar_animation {
            if animation.is_finished(now) {
                self.files_sidebar_animation = None;
                if !self.files_sidebar_open {
                    self.clear_files_sidebar_state();
                }
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.files_sidebar_open {
            FILES_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    fn set_files_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.files_sidebar_width_at(now);
        self.files_sidebar_open = open;
        if let Some(ws) = self.active_workspace_mut() {
            ws.active_tab_mut().files_sidebar_open = open;
        }
        let to_width = if open { FILES_SIDEBAR_WIDTH } else { 0. };

        self.files_sidebar_animation =
            if (from_width - to_width).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA {
                Some(crate::SidebarWidthAnimation {
                    from_width,
                    to_width,
                    started_at: now,
                })
            } else {
                None
            };

        if !open && self.files_sidebar_animation.is_none() {
            self.clear_files_sidebar_state();
        }
        cx.notify();
    }

    fn clear_files_sidebar_state(&mut self) {
        self.files_tree = FilesTreeState::default();
        self.files_watcher = None;
        self.files_event_rx = None;
        self.files_menu_open = None;
        self.files_surface_id = None;
        self.files_selected = 0;
    }

    pub(crate) fn sync_files_sidebar_session(&mut self, cx: &mut Context<Self>) {
        let wanted = self
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().files_sidebar_open);
        if wanted != self.files_sidebar_open {
            if wanted {
                self.toggle_files_sidebar(cx);
            } else {
                self.close_files_sidebar(cx);
            }
            return;
        }
        self.reroot_files_tree(cx);
    }

    fn reroot_files_tree(&mut self, cx: &mut Context<Self>) {
        if !self.files_sidebar_open {
            return;
        }
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        if self.files_tree.root == *Path::new(&ws.cwd) {
            return;
        }
        let root = PathBuf::from(&ws.cwd);
        let persisted = ws.files_expanded.clone();
        self.spawn_files_hydration(root, persisted, cx);
    }

    fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.files_tree.expanded.contains(path) {
            self.files_tree.expanded.remove(path);
            self.unwatch_files_dir(path);
        } else {
            self.files_tree.expanded.insert(path.to_path_buf());
            self.watch_files_dir(path);
            let stale =
                self.files_watcher.is_none() || !self.files_tree.children.contains_key(path);
            if stale {
                let listing = files_tree::read_dir_sorted(&self.files_tree.root, path);
                self.files_tree.children.insert(path.to_path_buf(), listing);
            }
        }
        self.sync_files_expansion();
        self.clamp_files_selection();
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn open_file_in_diff_dock(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section.is_some() {
            self.close_settings(cx);
        }
        self.enter_cli_mode(window, cx);
        if !self.diff_dock.open {
            let cwd = self.files_tree.root.to_string_lossy().into_owned();
            self.open_diff_dock_panel(cwd, cx);
        }
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.open_diff_file_tab(path, window, cx);
    }

    pub(crate) fn render_files_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        div()
            .id("files-sidebar")
            .flex()
            .flex_col()
            .w(SIDEBAR_WIDTH)
            .flex_shrink_0()
            .h_full()
            .track_focus(&self.files_focus)
            .on_key_down(cx.listener(Self::handle_files_sidebar_key_down))
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .child(self.files_sidebar_header(ui, cx))
            .child(self.files_sidebar_body(ui, cx))
            .into_any_element()
    }
}
