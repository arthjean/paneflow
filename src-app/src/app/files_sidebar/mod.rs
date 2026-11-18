mod context_menu;
mod integration;
mod keyboard;
mod list;
mod open_with;
mod panel;
#[cfg(test)]
mod perf_bench;
mod projection;
mod row;
mod search;
mod view;
mod watch;
mod worker;

use std::path::PathBuf;

use gpui::{AppContext, Context, Focusable, Pixels, Window, px};

use crate::{PaneFlowApp, ToggleFilesSidebar};
pub(crate) use panel::{FilesEvent, FilesSidebar};

pub(crate) const FILES_TREE_DEFAULT_WIDTH: f32 = 256.;
pub(crate) const FILES_TREE_MIN_WIDTH: f32 = 160.;
pub(crate) const FILES_TREE_MAX_WIDTH: f32 = 560.;
pub(crate) const FILES_TREE_CONTENT_MIN_WIDTH: f32 = 256.;
pub(crate) const FILES_TREE_KEY_STEP: f32 = 16.;
pub(super) const ROW_HEIGHT: Pixels = px(28.);
pub(super) const INDENT_STEP: f32 = 18.;
pub(super) const ROW_SLOT: f32 = 14.;
pub(super) const ROW_GAP: f32 = 12.;
pub(super) const DIMMED_OPACITY: f32 = 0.55;

impl PaneFlowApp {
    pub(crate) fn files_sidebar_host_visible(&self) -> bool {
        self.settings_section.is_none()
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
        self.sync_files_sidebar_session(cx);
        if self.diff_dock_visible() && self.diff_files_tab_active() && self.files_sidebar_open {
            let index = self.diff_dock.diff_active_tab;
            self.remove_diff_tab(index, cx);
            self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
            return;
        }
        if self.diff_dock_visible() && self.diff_file_tab_active() && self.files_sidebar_open {
            self.close_files_sidebar(cx);
            self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
            return;
        }
        if !self.diff_dock.open {
            let Some(cwd) = self.active_checkout() else {
                return;
            };
            self.open_diff_dock_panel(cwd, cx);
        }
        let file_index = self
            .diff_file_tab_active()
            .then_some(self.diff_dock.diff_active_tab)
            .or_else(|| {
                self.diff_dock.diff_tabs.iter().position(|tab| {
                    matches!(
                        tab,
                        super::diff_dock::DiffDockTab::File(_)
                            | super::diff_dock::DiffDockTab::PendingFile
                    )
                })
            });
        if let Some(index) = file_index {
            self.select_diff_tab(index, cx);
            if !self.files_sidebar_open {
                self.toggle_files_sidebar(cx);
            }
            self.focus_files_sidebar(window, cx);
        } else {
            self.open_diff_file_picker(window, cx);
        }
    }

    pub(crate) fn reveal_active_file_in_tree(&mut self, cx: &mut Context<Self>) {
        if !self.files_sidebar_open {
            return;
        }
        let Some(super::diff_dock::DiffDockTab::File(view)) =
            self.diff_dock.diff_tabs.get(self.diff_dock.diff_active_tab)
        else {
            if self.files_sidebar.read(cx).reveal_request.is_some() {
                self.files_sidebar
                    .update(cx, |panel, _| panel.reveal_request = None);
            }
            return;
        };
        let path = view.read(cx).path().to_path_buf();
        self.files_sidebar
            .update(cx, |panel, cx| panel.reveal_file(&path, cx));
    }

    pub(crate) fn focus_files_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.files_sidebar
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(crate) fn toggle_files_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.files_sidebar_open {
            self.close_files_sidebar(cx);
            return;
        }
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        let persisted = ws.files_expanded.clone();
        self.files_sidebar_workspace = Some(ws.id);
        self.dismiss_transient_surfaces();
        self.set_files_sidebar_open(true, cx);
        self.files_sidebar_root = Some(root.clone());
        self.files_sidebar
            .update(cx, |panel, cx| panel.open(root, persisted, cx));
        self.reveal_active_file_in_tree(cx);
    }

    pub(crate) fn close_files_sidebar(&mut self, cx: &mut Context<Self>) {
        self.files_sidebar.update(cx, |panel, _| panel.deactivate());
        self.files_menu_open = None;
        self.files_sidebar_root = None;
        self.files_sidebar_workspace = None;
        self.set_files_sidebar_open(false, cx);
        if self.files_sidebar_animation.is_none() {
            self.files_sidebar
                .update(cx, |panel, cx| panel.release_snapshot(cx));
        }
    }

    fn files_sidebar_progress_at(&self, now: std::time::Instant) -> f32 {
        if let Some(animation) = self.files_sidebar_animation {
            animation.width_at(now)
        } else if self.files_sidebar_open {
            1.
        } else {
            0.
        }
    }

    pub(crate) fn rendered_files_sidebar_progress(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> f32 {
        let now = std::time::Instant::now();
        if let Some(animation) = self.files_sidebar_animation {
            if animation.is_finished(now) {
                self.files_sidebar_animation = None;
                if !self.files_sidebar_open {
                    self.files_sidebar
                        .update(cx, |panel, cx| panel.release_snapshot(cx));
                }
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.files_sidebar_open {
            1.
        } else {
            0.
        }
    }

    fn set_files_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.files_sidebar_progress_at(now);
        self.files_sidebar_open = open;
        if let Some(ws) = self.active_workspace_mut() {
            ws.active_tab_mut().files_sidebar_open = open;
        }
        self.save_session(cx);
        let to_width = if open { 1. } else { 0. };
        self.files_sidebar_animation =
            if !crate::ui_primitives::reduce_motion() && (from_width - to_width).abs() > 0.01 {
                Some(crate::SidebarWidthAnimation {
                    from_width,
                    to_width,
                    started_at: now,
                })
            } else {
                None
            };
        cx.notify();
    }

    pub(crate) fn sync_files_sidebar_session(&mut self, cx: &mut Context<Self>) {
        self.sync_diff_dock_session(cx);
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
        if !self.files_sidebar_open {
            return;
        }
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        if self.files_sidebar_root.as_ref() == Some(&root)
            && self.files_sidebar_workspace == Some(ws.id)
        {
            self.reveal_active_file_in_tree(cx);
            return;
        }
        let persisted = ws.files_expanded.clone();
        self.files_sidebar_workspace = Some(ws.id);
        self.files_sidebar_root = Some(root.clone());
        self.files_menu_open = None;
        self.files_sidebar
            .update(cx, |panel, cx| panel.open(root, persisted, cx));
        self.reveal_active_file_in_tree(cx);
    }

    pub(crate) fn begin_files_tree_resize(
        &mut self,
        cursor_x: f32,
        shown_width: f32,
        cx: &mut Context<Self>,
    ) {
        self.files_tree_resize = Some((cursor_x, clamp_files_tree_width(shown_width)));
        cx.notify();
    }

    pub(crate) fn drag_files_tree_resize(&mut self, cursor_x: f32, cx: &mut Context<Self>) {
        let Some((anchor_x, anchor_width)) = self.files_tree_resize else {
            return;
        };
        let width = clamp_files_tree_width(anchor_width + anchor_x - cursor_x);
        if width != self.files_tree_width {
            self.files_tree_width = width;
            cx.notify();
        }
    }

    pub(crate) fn end_files_tree_resize(&mut self, cx: &mut Context<Self>) -> bool {
        if self.files_tree_resize.take().is_none() {
            return false;
        }
        self.persist_files_tree_width(cx);
        cx.notify();
        true
    }

    pub(crate) fn step_files_tree_width(
        &mut self,
        step: FilesTreeStep,
        shown_width: f32,
        cx: &mut Context<Self>,
    ) {
        let width = stepped_files_tree_width(shown_width, step);
        if width != self.files_tree_width {
            self.files_tree_width = width;
            self.persist_files_tree_width(cx);
            cx.notify();
        }
    }

    fn persist_files_tree_width(&self, cx: &mut Context<Self>) {
        crate::window_state::record_files_tree_width(Some(self.files_tree_width));
        cx.background_spawn(async { crate::window_state::save() })
            .detach();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilesTreeStep {
    Wider,
    Narrower,
    Minimum,
    Maximum,
}

impl FilesTreeStep {
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "left" => Some(Self::Wider),
            "right" => Some(Self::Narrower),
            "home" => Some(Self::Minimum),
            "end" => Some(Self::Maximum),
            _ => None,
        }
    }
}

pub(crate) fn clamp_files_tree_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(FILES_TREE_MIN_WIDTH, FILES_TREE_MAX_WIDTH)
    } else {
        FILES_TREE_DEFAULT_WIDTH
    }
}

pub(crate) fn valid_files_tree_width(width: f32) -> bool {
    width.is_finite() && (FILES_TREE_MIN_WIDTH..=FILES_TREE_MAX_WIDTH).contains(&width)
}

pub(crate) fn stepped_files_tree_width(width: f32, step: FilesTreeStep) -> f32 {
    match step {
        FilesTreeStep::Wider => clamp_files_tree_width(width + FILES_TREE_KEY_STEP),
        FilesTreeStep::Narrower => clamp_files_tree_width(width - FILES_TREE_KEY_STEP),
        FilesTreeStep::Minimum => FILES_TREE_MIN_WIDTH,
        FilesTreeStep::Maximum => FILES_TREE_MAX_WIDTH,
    }
}

pub(crate) fn files_tree_fit(preferred: f32, available: f32) -> f32 {
    let preferred = clamp_files_tree_width(preferred);
    let room = (available - FILES_TREE_CONTENT_MIN_WIDTH).max(0.);
    preferred.min(room).max(FILES_TREE_MIN_WIDTH)
}

#[cfg(test)]
mod tests;
