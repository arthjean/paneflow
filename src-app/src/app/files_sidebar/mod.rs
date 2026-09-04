mod context_menu;
mod filter;
mod integration;
mod keyboard;
mod list;
mod panel;
mod projection;
mod row;
mod view;
mod watch;
mod worker;

use std::path::PathBuf;

use gpui::{Context, Focusable, Pixels, Window, px};

use crate::{PaneFlowApp, ToggleFilesSidebar};
pub(crate) use panel::{FilesEvent, FilesSidebar};

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
        self.toggle_files_sidebar(cx);
        if self.files_sidebar_open {
            self.focus_files_sidebar(window, cx);
        }
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
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        let persisted = ws.files_expanded.clone();
        self.files_sidebar_workspace = Some(ws.id);
        if self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some()
        {
            self.close_sessions_sidebar_immediate(cx);
        }
        self.dismiss_transient_surfaces();
        self.set_files_sidebar_open(true, cx);
        self.files_sidebar_root = Some(root.clone());
        self.files_sidebar
            .update(cx, |panel, cx| panel.open(root, persisted, cx));
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

    fn files_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if let Some(animation) = self.files_sidebar_animation {
            animation.width_at(now)
        } else if self.files_sidebar_open {
            FILES_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(crate) fn rendered_files_sidebar_width(
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
        cx.notify();
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
        if !self.files_sidebar_open {
            return;
        }
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        let root = PathBuf::from(&ws.cwd);
        if self.files_sidebar_root.as_ref() == Some(&root)
            && self.files_sidebar_workspace == Some(ws.id)
        {
            return;
        }
        let persisted = ws.files_expanded.clone();
        self.files_sidebar_workspace = Some(ws.id);
        self.files_sidebar_root = Some(root.clone());
        self.files_menu_open = None;
        self.files_sidebar
            .update(cx, |panel, cx| panel.open(root, persisted, cx));
    }
}

#[cfg(test)]
mod tests;
