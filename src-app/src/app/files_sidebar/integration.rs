use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, IntoElement, StyleRefinement, Styled, Window, px};

use super::{FilesEvent, FilesSidebar};
use crate::PaneFlowApp;

impl PaneFlowApp {
    pub(crate) fn handle_files_event(
        &mut self,
        _: Entity<FilesSidebar>,
        event: &FilesEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            FilesEvent::Close(window) => {
                self.close_files_sidebar(cx);
                let host = cx.weak_entity();
                let window = *window;
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.focus_diff_tab(app.diff_dock.diff_active_tab, window, cx);
                        });
                    });
                });
            }
            FilesEvent::ContextMenu(menu) => {
                self.dismiss_transient_surfaces();
                self.files_menu_open = Some(menu.clone());
                cx.notify();
            }
            FilesEvent::Expanded { root, paths } => {
                if let Some(ws) = self.active_workspace_mut()
                    && Path::new(&ws.cwd) == root
                    && ws.files_expanded != *paths
                {
                    ws.files_expanded = paths.clone();
                    self.save_session(cx);
                }
            }
            FilesEvent::OpenFile { path, root, window } => {
                let host = cx.weak_entity();
                let path = path.clone();
                let root = root.clone();
                let window = *window;
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.open_file_in_diff_dock(path, root, window, cx)
                        });
                    });
                });
            }
        }
    }

    fn open_file_in_diff_dock(
        &mut self,
        path: PathBuf,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_section.is_some() {
            self.close_settings(cx);
        }
        self.enter_cli_mode(window, cx);
        if !self.diff_dock.open {
            self.open_diff_dock_panel(root.to_string_lossy().into_owned(), cx);
        }
        self.diff_dock.picker = false;
        self.diff_dock.picked = true;
        self.open_diff_file_tab(path, window, cx);
    }

    pub(crate) fn render_files_sidebar(&self, width: f32) -> AnyElement {
        self.files_sidebar
            .clone()
            .cached(
                StyleRefinement::default()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0(),
            )
            .into_any_element()
    }
}
